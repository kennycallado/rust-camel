## ADDED Requirements

### Requirement: EIP catalog covers tier-1 deferred patterns

The guide SHALL document four tier-1 EIPs with include-backed example pages: Zip Splitter, Delayer, Loop, and Validator. Each page includes from an existing compiled example that gains an anchor comment pair.

#### Scenario: user reads Delayer page

- **GIVEN** the `eip/delayer.md` page
- **WHEN** the user reads the include block
- **THEN** the page shows the delay step from `examples/delayer/`
- **AND** the prose explains the fixed-duration delay semantics

#### Scenario: tier-1 examples still compile after anchor addition

- **GIVEN** the 4 existing examples that gain anchor comments
- **WHEN** `cargo build` runs on each
- **THEN** each build succeeds with zero errors

### Requirement: EIP catalog covers tier-2 patterns with new examples

The guide SHALL document six tier-2 EIPs with include-backed example pages: Idempotent Consumer, Content Enricher, Claim Check, Sort, Sampling, and Resequencer. Each page includes from a new compiled example created for this change.

#### Scenario: user reads Idempotent Consumer page

- **GIVEN** the `eip/idempotent-consumer.md` page
- **WHEN** the user reads the include block
- **THEN** the page shows the idempotent consumer step from a compiled example
- **AND** the prose explains the outcome-aware duplicate-rejection behavior and the `IdempotentRepository` trait

#### Scenario: all 6 new examples compile

- **GIVEN** the 6 new example directories
- **WHEN** `cargo build` runs on each
- **THEN** each build succeeds with zero errors

### Requirement: EIP hub has zero deferred entries

The `eip/index.md` page SHALL list all 27 EIP pages under four family headings with no Deferred section. Every pattern that previously appeared in Deferred now has a dedicated page linked from its family.

#### Scenario: hub has no deferred section

- **GIVEN** the rewritten `eip/index.md`
- **WHEN** the user opens the EIP hub
- **THEN** four family headings appear (Routing, Transformation, Messaging, Resilience and control)
- **AND** no Deferred section exists

### Requirement: SUMMARY lists all 27 EIP pages

The `docs/src/SUMMARY.md` file SHALL list all 27 EIP pages (17 existing plus 10 new) under the EIP section.

#### Scenario: SUMMARY contains 27 entries

- **GIVEN** the updated `docs/src/SUMMARY.md`
- **WHEN** a tool reads the EIP section
- **THEN** 27 indented page entries appear

### Requirement: Guide builds with zero errors and zero broken links

The mdBook guide SHALL build with exit code 0, and all cross-page Markdown links SHALL resolve.

#### Scenario: mdbook build succeeds

- **GIVEN** all 10 new EIP pages and the rewritten hub
- **WHEN** `mdbook build docs` runs
- **THEN** the build exits with code 0 and emits zero broken-link warnings

### Requirement: All modified and new examples compile

Every example that receives anchor comments or is newly created SHALL compile without errors. YAML-backed examples SHALL additionally pass a route parse and compile check to satisfy the compiled-example two-source rule.

#### Scenario: all 10 examples compile

- **GIVEN** all 4 existing examples with new anchors plus 6 new examples
- **WHEN** `cargo build` runs on each
- **THEN** each build succeeds with zero errors

#### Scenario: YAML route examples parse and compile

- **GIVEN** any new example that uses a YAML route file instead of Rust RouteBuilder
- **WHEN** the example's `main.rs` loads the YAML via `camel_dsl::yaml::load_from_file`, registers it via `add_route_definition(...)`, and calls `CamelContext::start()`
- **THEN** the route compiles and starts without errors
