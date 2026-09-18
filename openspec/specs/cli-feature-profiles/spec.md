# cli-feature-profiles Specification

## Purpose
TBD - created by archiving change clidiet. Update Purpose after archive.
## Requirements
### Requirement: Default feature closure is unchanged

camel-cli's default build SHALL reproduce, feature-for-feature, the
dependency closure of the pre-change default set. The re-aggregation
(`default = ["flavor-regular"]`, which forwards `full`; camel-core
consumed without language features and re-enabled through `lang-*`
forwards) MUST NOT change which crates, crate versions, or forwarded
features resolve in the default graph, and MUST NOT change
default-binary behavior.

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

### Requirement: Flavor markers are the single selection surface

camel-cli SHALL expose exactly three flavor marker features —
`flavor-slim`, `flavor-regular`, `flavor-full` — that forward to the
profile composition and serve as the single source of truth for
profile selection in release CI. `default` SHALL select
`flavor-regular`. Marker bodies in this change are aliases only:
`flavor-slim` forwards `slim-http`; `flavor-regular` forwards `full`;
`flavor-full` forwards `full` and `kafka`. Flavor content curation
(what each marker contains beyond these aliases) is out of scope here
and belongs to the flavor-matrix change. When several markers are
enabled simultaneously, the reported flavor SHALL follow the priority
full > regular > slim; builds with no marker enabled report `custom`.

#### Scenario: marker closures are aliases

- **GIVEN** the camel-cli feature table
- **WHEN** `cargo tree` resolves `--features flavor-full` and
  separately `--features full,kafka`; likewise `flavor-regular`
  vs `full`, and `--no-default-features --features flavor-slim`
  vs `--no-default-features --features slim-http`
- **THEN** each marker's resolved package set is identical to its
  forwarding target's package set

#### Scenario: default closure unchanged

- **GIVEN** the golden fixture pinning the default closure
  (`default_closure_matches_golden`)
- **WHEN** `default` is changed from `["full"]` to
  `["flavor-regular"]`
- **THEN** the default closure is byte-identical and the golden
  fixture passes without regeneration

#### Scenario: unmarked builds report custom

- **GIVEN** a build with `--no-default-features --features slim-http`
  (raw composition, no flavor marker)
- **WHEN** the binary reports its version
- **THEN** the flavor suffix is `custom`

### Requirement: version output reports the flavor

The interactive `camel --version` (the non-artifact CLI path, served by
the clap `version` attribute in `crates/camel-cli/src/main.rs`) SHALL
report the crate version followed by the compile-time flavor in a
parseable suffix: `<semver> (<flavor>)`, e.g. `camel 0.49.0 (regular)`.
The flavor SHALL be computed from the enabled marker cfgs with the
documented priority. The compiled-artifact manifest's `RUNTIME_VERSION`
SHALL stay semver-only (the suffix is CLI presentation, not manifest
data). The compiled-artifact runtime `--version` path
(`crates/camel-cli/src/compile/runtime.rs`, reached via
`self_detect_artifact` before clap parses) is OUT OF SCOPE for the
suffix in this change: it keeps printing `camel <semver>` (bare) so the
manifest-schema invariant (`RUNTIME_VERSION` semver-only, machine-read
by `--manifest`) is not weakened by CLI presentation. Adding the flavor
suffix to the artifact trailer path, if triage later needs it, is a
follow-up that MUST keep `RUNTIME_VERSION` and the `--manifest` JSON
semver-only and add the flavor as a separate presentation-only token.

#### Scenario: default build reports regular

- **GIVEN** a default-features build of camel-cli
- **WHEN** the interactive `camel --version` runs (non-artifact CLI)
- **THEN** the output line matches `<semver> (regular)`

#### Scenario: compiled-artifact --version stays bare

- **GIVEN** a `camel compile` artifact built from a flavored binary
- **WHEN** the artifact's `--version` runs (the pre-clap
  `self_detect_artifact` path)
- **THEN** the output line is `camel <semver>` with no flavor suffix,
  and the `--manifest` JSON `runtime_version` field stays semver-only

#### Scenario: marker build reports its flavor

- **GIVEN** builds with `--features flavor-full`, with
  `--no-default-features --features flavor-slim`, and with
  `--features flavor-regular`
- **WHEN** each binary's `--version` runs
- **THEN** the suffixes are `(full)`, `(slim)`, and `(regular)`
  respectively

### Requirement: release legs select a flavor marker, not composed feature lists

The release workflow's build step SHALL select each leg's feature set
by exactly one flavor marker (plus the allocator feature on legs that
need it) and SHALL NOT compose feature lists by string surgery over
multiple matrix keys. The per-leg closures SHALL be identical to the
pre-change closures. Existing post-build assertions (kafka capability
probe, jemalloc link assert) SHALL remain unchanged and green.

#### Scenario: kafka legs build flavor-full

- **GIVEN** the five kafka-capable release legs
  (x86_64-gnu, aarch64-gnu, both macOS, windows-msvc)
- **WHEN** the workflow builds them
- **THEN** the build command carries `--features flavor-full` (no
  kafka-features matrix key, no sed composition) and the closure
  matches the previous default+kafka set

#### Scenario: musl legs build flavor-regular plus allocator

- **GIVEN** the two musl legs
- **WHEN** the workflow builds them
- **THEN** the build command carries `--features flavor-regular,jemalloc`
  and the closure matches the previous default+jemalloc set

#### Scenario: version flavor probed per leg

- **GIVEN** any release leg whose binary is executable on its runner
- **WHEN** the post-build version check runs
- **THEN** the printed flavor suffix equals the leg's matrix flavor

