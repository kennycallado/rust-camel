## MODIFIED Requirements

### Requirement: Bridge features are individually selectable

Each controllable optional surface SHALL be re-selectable in slim and
full builds through its camel-cli feature (`kafka`, `grpc`, `wasm`,
`llm`, `mcp`, `mqtt`, `surrealdb`, `exec`, `lsp`, `lang-js`,
`lang-rhai`, `lang-jsonpath`, `lang-xpath`, `lang-minijinja`),
composing additively with `slim-http`. For kafka, camel-cli SHALL
expose exactly two features: `kafka` (capability: component activation
plus registration in the lint registry and the boot cascade, with
librdkafka built from source) and `dynamic-linking` (capability plus
system-librdkafka linking, implying `kafka`). The historical
`cmake-build` and `kafka-static` names SHALL NOT exist as camel-cli
features.

#### Scenario: slim plus one bridge resolves that bridge only

- **GIVEN** a build configured `--no-default-features --features slim-http,grpc`
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

## ADDED Requirements

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
