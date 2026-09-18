# cli-feature-profiles Specification

## Purpose
TBD - created by archiving change clidiet. Update Purpose after archive.
## Requirements
### Requirement: Default feature closure is unchanged

camel-cli's default build SHALL reproduce, feature-for-feature, the
dependency closure of the pre-change default set. The re-aggregation
(`default = ["full"]`, camel-core consumed without language features and
re-enabled through `lang-*` forwards) MUST NOT change which crates,
crate versions, or forwarded features resolve in the default graph, and
MUST NOT change default-binary behavior.

#### Scenario: golden dependency snapshot matches

- **GIVEN** a committed golden snapshot of the default closure at the
  change base, generated with
  `cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | sort -u`
  (package lines plus feature-edge lines)
- **WHEN** the profile test regenerates the closure on the changed tree
  with the same command, filtering BOTH sides (live and golden) of
  `camel-core feature "lang-*"` and `camel-core feature "camel-language-*"`
  lines — cargo tree renders feature nodes for dep-declaration activation
  but not for feature-forwarding, and the `lang-*` features moved from
  declaration to forwarding by design
- **THEN** the normalized sorted lists are identical; package-level
  presence of every `camel-language-*` crate in the default closure is
  still asserted by the surviving package lines

#### Scenario: default binary shows no regression within stated bands

- **GIVEN** baseline measurements of the pre-change default release
  binary: marker-mode startup median, help-mode startup median, max-RSS
  median, and exact byte size (n=30 per mode)
- **WHEN** the post-change default release binary is measured with the
  same local method and sample count
- **THEN** each startup median is within ±5% of its baseline median,
  each max-RSS median is within ±2 MB of its baseline median, and the
  byte-size delta is zero or explained in the recorded evidence

### Requirement: Slim profile excludes the controllable bridge set

camel-cli SHALL provide a `slim-benchmarks` feature naming the
`--no-default-features` baseline: an http-only deployment that links the
non-optional core plumbing (http, direct, seda, log, mock, timer, file,
controlbus, container, validator, cron, and the DSL/config/core stack)
and SHALL NOT link the camel-cli-controllable optional set — kafka,
grpc, wasm, llm, mcp, mqtt, surrealdb, exec, the lsp stack (camel-lsp,
tower-lsp), and the language runtimes gated by the `lang-*`
features (js, rhai, jsonpath, xpath) — when unselected. The historical
`slim-http` name SHALL live exactly one release as a forwarding alias
(`slim-http = ["slim-benchmarks"]`, drop at 0.50).
`camel-language-minijinja` remains linked in every profile: the
workspace consumes camel-template with default features, and the
in-workspace default-features flip rides the camel-cli bridge-forward
mission (the camel-template engine itself is optional at the source as
of mission 115). The eight bridges (jms, sql, redis, opensearch, ws,
cxf, xslt, xj) remain linked in slim via camel-cli's OWN unconditional
dependencies; camel-bundles' side is optional as of mission 115 (the
http-static carrier), and the camel-cli-side diet is deferred to the
camel-cli bridge-forward mission. They are out of this requirement's
exclusion set until that mission lands.

#### Scenario: slim closure excludes the controllable set

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks`
- **WHEN** `cargo tree -p camel-cli --no-default-features -e no-dev`
  resolves
- **THEN** none of camel-component-{kafka,grpc,wasm,llm,mcp,mqtt,
  surrealdb,exec}, camel-lsp, tower-lsp, or the excluded language-runtime
  crates (js, rhai, jsonpath, xpath — NOT minijinja, see the requirement
  text) appears in the graph (ariadne may appear: `camel lint` keeps it
  non-optional)

#### Scenario: slim build compiles and boots an http route

- **GIVEN** the slim-benchmarks release binary
- **WHEN** it boots the cold-start fixture (an http consumer route that
  must bind, plus the one-shot timer→log marker route)
- **THEN** it reaches the route-ready marker, proving the profile is
  self-coherent (no dangling forward such as `otel` without the http
  bridge, and the LSP-free binary still parses and boots routes)

### Requirement: Bridge features are individually selectable

Each controllable optional surface SHALL be re-selectable in slim and
full builds through its camel-cli feature (`kafka`, `grpc`, `wasm`,
`llm`, `mcp`, `mqtt`, `surrealdb`, `exec`, `lsp`, `lang-js`,
`lang-rhai`, `lang-jsonpath`, `lang-xpath`, `lang-minijinja`),
composing additively with `slim-benchmarks` (and, for one release, the
`slim-http` alias). For kafka, camel-cli SHALL
expose exactly two features: `kafka` (capability: component activation
plus registration in the lint registry and the boot cascade, with
librdkafka built from source) and `dynamic-linking` (capability plus
system-librdkafka linking, implying `kafka`). The historical
`cmake-build` and `kafka-static` names SHALL NOT exist as camel-cli
features.

#### Scenario: slim plus one bridge resolves that bridge only

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks,grpc`
- **WHEN** the dependency graph resolves
- **THEN** the grpc stack (camel-component-grpc, tonic) is present and
  the remaining controllable set stays excluded

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

### Requirement: Kafka release legs assert capability against the built binary

Release builds that enable a kafka capability feature SHALL verify,
before artifacts are published, that the built binary resolves `kafka:`
endpoints, via a lint probe over a fixture route whose source is a
syntactically valid `kafka:` endpoint carrying a `brokers=` option
(`kafka:orders?brokers=localhost:9092`). The normative gate is
two-part, and both parts are load-bearing: lint exits 0 AND no
`unverified-scheme` diagnostic appears in the lint output — the
matcher keys on the diagnostic code, not on word co-occurrence with
"kafka" (diagnostics never name the scheme on the header line, and
ANSI escapes split the echoed scheme token; the fixture's only
capability-gated scheme is kafka, so an `unverified-scheme` hit names
kafka by construction). Exit code alone cannot discriminate — an
unregistered scheme surfaces as an Info-severity diagnostic that also
exits 0 — and a registered component over a malformed endpoint errors
into exit 1. The probe SHALL run on release
legs whose binary is natively executable on its runner and SHALL be
absent from the cross-compiled aarch64-gnu leg (feature registration
is compile-time cfg, target-independent, and the identical feature
string is exercised on a native leg). Legs built without kafka
capability stay kafka-less, as already asserted by the camel-bundles
gating tests.

#### Scenario: kafka leg probe is green

- **GIVEN** a release CI leg built with `--features kafka` on a
  native-executable runner (x86_64-gnu, macOS, or Windows)
- **WHEN** the built binary lints a fixture route whose source
  endpoint is `kafka:orders?brokers=localhost:9092`
- **THEN** lint exits 0 AND no `unverified-scheme` diagnostic appears
  in the output (both normative)

#### Scenario: kafka-less legs remain kafka-less

- **GIVEN** a release leg built without any kafka capability feature
  (musl targets)
- **WHEN** the binary is produced
- **THEN** no kafka registration is compiled in, as asserted by the
  existing camel-bundles feature-gating tests (negative polarity)

### Requirement: camel-bundles bridge optionality

camel-bundles SHALL expose the eight bridge crates (camel-component-jms,
camel-component-sql, camel-component-redis, camel-component-opensearch,
camel-component-ws, camel-component-cxf, camel-xj, camel-xslt) as optional
dependencies activated by `dep:` entries on the existing `http-static`
feature, WITHOUT adding any new key to camel-bundles' `[features]` table
(the gate-forwarding lint requires boot consumers to forward every feature
key, and camel-cli's dependency wiring — forbidden at design time — remains
excluded from this change even after the bounded rename-only grant recorded
below). camel-bundles' `default`
set is unchanged and still contains `http-static`, so default-feature
consumers keep the exact pre-change dependency closure and boot
registration.

#### Scenario: default closure byte-identical

- **GIVEN** camel-cli built with default features on a tree where the bridges
  are optionalized per this requirement
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
  deferral), and enabling `http-static` alone restores all eight

#### Scenario: transitional carrier keeps default builds registering bridges

- **GIVEN** camel-cli's `full` feature forwards `camel-bundles/http-static`
- **WHEN** the http-static feature is enabled
- **THEN** the eight bridge dependencies are active and `boot()` registers
  the identical component set as before the change

#### Scenario: gate-forwarding lint stays green

- **GIVEN** camel-bundles' `[features]` table keys unchanged by this
  requirement
- **WHEN** `cargo xtask lint-gate-forwarding` runs
- **THEN** it exits zero (no boot-consumer forwarding violations)

### Requirement: gated boot registration and pool handles

camel-bundles' `boot()` SHALL cfg-gate each bridge registration site on
`feature = "http-static"`, and the `BootHandle` jms/cxf pool fields and their
shutdown drain steps SHALL be cfg-gated identically. The always-on
`BootHandle` surface (`datasource_catalog()`, `shutdown()`,
`shutdown_with_deadline()`) SHALL compile and behave identically in every
feature combination, and no external consumer of camel-bundles SHALL need
source changes for any feature combination camel-cli can express.

#### Scenario: feature-off boot succeeds

- **GIVEN** camel-bundles compiled with `--no-default-features`
- **WHEN** `boot()` runs against a fixture that references no bridge
- **THEN** boot completes, core components register, and shutdown drains
  without the gated pools

#### Scenario: default boot registers all bridges

- **GIVEN** camel-bundles compiled with default features
- **WHEN** `boot()` runs against the bundles-present fixture
- **THEN** the eight bridge components register exactly as before the change
  (camel-xj registration accompanied by camel-xslt, matching the pre-change
  transitive dependency)

### Requirement: slim-benchmarks profile rename

camel-cli's `slim-http` marker feature SHALL be renamed `slim-benchmarks`
(owner ruling 2026-09-17: the slim profile is benchmark-oriented), with
`slim-http = ["slim-benchmarks"]` retained as a one-release alias so existing
build legs keep resolving until the alias is dropped.

#### Scenario: renamed profile excludes the controllable set

- **GIVEN** camel-cli resolved with `--no-default-features --features
  slim-benchmarks`
- **WHEN** the closure is compared against the forbidden-prefix set
- **THEN** all forbidden prefixes are absent, identical to the pre-rename
  slim-http closure

#### Scenario: alias resolves for one release

- **GIVEN** camel-cli resolved with `--no-default-features --features
  slim-http` (the alias)
- **WHEN** the closure is listed
- **THEN** it is identical to the slim-benchmarks closure (alias forwards to
  the renamed marker; no separate surface)

#### Scenario: golden deptree unaffected by the rename

- **GIVEN** the renamed feature and alias in camel-cli's `[features]`
- **WHEN** `cargo tree -p camel-cli` (default features) is compared against
  the golden fixture
- **THEN** the comparison passes without regeneration (unused marker/alias
  features render no feature lines)

