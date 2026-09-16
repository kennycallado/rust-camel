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

camel-cli SHALL provide a `slim-http` feature naming the
`--no-default-features` baseline: an http-only deployment that links the
non-optional core plumbing (http, direct, seda, log, mock, timer, file,
controlbus, container, validator, cron, and the DSL/config/core stack)
and SHALL NOT link the camel-cli-controllable optional set — kafka,
grpc, wasm, llm, mcp, mqtt, surrealdb, exec, the lsp stack (camel-lsp,
tower-lsp), and the language runtimes gated by the `lang-*`
features (js, rhai, jsonpath, xpath) — when unselected.
`camel-language-minijinja` remains linked in every profile (see the
requirement text below for the camel-template hard edge). The
bundles-unconditional bridges (jms, sql, redis, opensearch, ws, cxf,
xslt, xj) remain linked in every profile until the deferred
camel-bundles-side optionalization lands; they are out of this
requirement's exclusion set.

#### Scenario: slim closure excludes the controllable set

- **GIVEN** a build configured `--no-default-features --features slim-http`
- **WHEN** `cargo tree -p camel-cli --no-default-features -e no-dev`
  resolves
- **THEN** none of camel-component-{kafka,grpc,wasm,llm,mcp,mqtt,
  surrealdb,exec}, camel-lsp, tower-lsp, or the excluded language-runtime
  crates (js, rhai, jsonpath, xpath — NOT minijinja, see the requirement
  text) appears in the graph (ariadne may appear: `camel lint` keeps it
  non-optional)

#### Scenario: slim build compiles and boots an http route

- **GIVEN** the slim-http release binary
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
composing additively with `slim-http`.

#### Scenario: slim plus one bridge resolves that bridge only

- **GIVEN** a build configured `--no-default-features --features slim-http,grpc`
- **WHEN** the dependency graph resolves
- **THEN** the grpc stack (camel-component-grpc, tonic) is present and
  the remaining controllable set stays excluded

