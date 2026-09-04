# gate-forwarding-lint Specification

## Purpose
TBD - created by archiving change gate-forwarding-assert. Update Purpose after archive.
## Requirements
### Requirement: Bundle gate forwarding lint

The `lint-gate-forwarding` xtask command SHALL verify that every
workspace crate depending on `camel-bundles` forwards the bundle gates
it names, and that boot consumers forward all of them, by static
manifest analysis only.

#### Scenario: clean tree passes

- **GIVEN** the workspace where every consumer feature named after a
  bundles gate transitively activates `camel-bundles/<gate>`, and every
  `boot-consumer`-marked crate forwards all 8 gates
- **WHEN** `cargo xtask lint-gate-forwarding` runs
- **THEN** it prints `lint-gate-forwarding: OK (0 violations)` and exits 0

#### Scenario: shadow feature without forwarding is a violation

- **GIVEN** a consumer manifest whose `[features]` contains
  `kafka = ["dep:camel-component-kafka"]` with no entry activating
  `camel-bundles/kafka`
- **WHEN** the lint runs
- **THEN** it reports one violation naming the crate path, the feature
  `kafka`, and the missing forwarding `camel-bundles/kafka`, and exits 1

#### Scenario: transitive forwarding counts

- **GIVEN** a consumer manifest whose `[features]` contains
  `kafka = ["bundle-kafka"]` and
  `bundle-kafka = ["camel-bundles/kafka"]`
- **WHEN** the lint resolves the closure of the gate-named feature
  `kafka`
- **THEN** no violation is reported, because the closure reaches
  `camel-bundles/kafka` through `bundle-kafka`

#### Scenario: dev-dependency makes a crate a consumer

- **GIVEN** a workspace manifest whose `[dev-dependencies]` contains a
  `camel-bundles` key and whose `[features]` contains a gate-named
  feature that does not forward the gate
- **WHEN** the lint runs
- **THEN** it reports the shadow-feature violation for that crate

#### Scenario: boot consumer missing one gate is a violation

- **GIVEN** a consumer manifest marked
  `[package.metadata.camel-bundles] boot-consumer = true` whose feature
  table forwards 7 of 8 gates
- **WHEN** the lint runs
- **THEN** it reports one violation for the unforwarded gate naming the
  crate and the gate, and exits 1

#### Scenario: unmarked consumer is exempt from completeness

- **GIVEN** `camel-integration-test`, which depends on `camel-bundles`
  with `default-features = false` and declares no boot-consumer marker
  and no gate-named feature
- **WHEN** the lint runs
- **THEN** it reports no violation for that crate

#### Scenario: crate outside the consumer set is ignored

- **GIVEN** a workspace crate with no `camel-bundles` dependency whose
  `[features]` contains a feature named `kafka`
- **WHEN** the lint runs
- **THEN** it reports no violation for that crate, because gate rules
  apply only to consumers of `camel-bundles`

#### Scenario: default is not a gate

- **GIVEN** `camel-bundles` `[features]` contains a `default` key
- **WHEN** the lint collects gates
- **THEN** the `default` key is excluded from the gate set

