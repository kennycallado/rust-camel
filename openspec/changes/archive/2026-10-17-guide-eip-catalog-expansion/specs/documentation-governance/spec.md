## ADDED Requirements

### Requirement: EIP catalog covers tier-1 routing patterns

The guide SHALL document seven routing EIPs with include-backed example pages: Content-Based Router (`choice`), Dynamic Router, Recipient List, Routing Slip, Wire Tap, Multicast, and Load Balancer.

#### Scenario: user browses routing EIPs

- **GIVEN** the EIP hub page at `eip/index.md`
- **WHEN** the user reads the Routing family section
- **THEN** all seven routing patterns appear as links to dedicated pages
- **AND** each page includes at least one `{{#include}}` from a compiled example

#### Scenario: Content-Based Router example compiles

- **GIVEN** the `examples/content-based-router/` directory
- **WHEN** `cargo build -p content-based-router` runs
- **THEN** the build succeeds with zero errors

### Requirement: EIP catalog covers tier-1 transformation patterns

The guide SHALL document three transformation EIP pages with include-backed example regions: Convert Body, Marshal/Unmarshal (consolidated), and Poll Enrich. The Marshal/Unmarshal page SHALL include from both `examples/marshal-csv/` and `examples/marshal-unmarshal/`. The Poll Enrich page SHALL include from `examples/file-pollenrich/`.

#### Scenario: user reads Marshal and Unmarshal page

- **GIVEN** the `eip/marshal-unmarshal.md` page
- **WHEN** the user reads the include blocks
- **THEN** the page shows marshal from `examples/marshal-csv/` AND unmarshal from `examples/marshal-unmarshal/`
- **AND** the prose explains the serialization format parameter

#### Scenario: user reads Poll Enrich page

- **GIVEN** the `eip/poll-enrich.md` page
- **WHEN** the user reads the include block
- **THEN** the page shows poll enrichment from `examples/file-pollenrich/`
- **AND** the prose explains how the consumer polls a resource and the default `UseEnrichedBody` strategy replaces the exchange body with the polled result

### Requirement: EIP catalog covers tier-1 messaging patterns

The guide SHALL document two messaging EIPs with include-backed example pages: Splitter and Streaming Splitter. The Streaming Splitter page SHALL cite ADR-0025 (outcome-aware structural EIPs) for structural segment behavior and source backpressure semantics from `crates/camel-processor/CONTEXT.md`.

#### Scenario: user reads Streaming Splitter page

- **GIVEN** the `eip/streaming-splitter.md` page
- **WHEN** the user reads the prose
- **THEN** the page cites ADR-0025 for structural outcome behavior
- **AND** sources backpressure details from the processor crate authority

### Requirement: EIP catalog covers tier-1 resilience patterns

The guide SHALL document two resilience EIPs with include-backed example pages: Do Try and Throttler.

#### Scenario: user reads Throttler page

- **GIVEN** the `eip/throttler.md` page
- **WHEN** the user reads the include block
- **THEN** the page shows the throttle step from `examples/throttler/`
- **AND** the prose explains the rate-limiting semantics

### Requirement: EIP hub groups pages by pattern family

The `eip/index.md` page SHALL organize all EIP pages under four family headings: Routing, Transformation, Messaging, and Resilience and control. Each heading lists its pattern pages as descriptive links.

#### Scenario: hub shows four family sections

- **GIVEN** the rewritten `eip/index.md`
- **WHEN** the user opens the EIP hub
- **THEN** four family headings appear with their pattern pages grouped beneath

### Requirement: Choice example demonstrates Content-Based Router

The repository SHALL contain `examples/content-based-router/` with a timer-driven route that uses the `choice` step with at least two `when` branches and one `otherwise` branch.

#### Scenario: choice example routes by body content

- **GIVEN** the `examples/content-based-router/src/main.rs` file
- **WHEN** the route processes an exchange
- **THEN** the `choice` step evaluates predicates and routes to the matching `when` branch or the `otherwise` fallback

### Requirement: Tier-2 and catalog-only patterns are listed but deferred

The EIP hub SHALL list tier-2 patterns (Zip Splitter, Delayer, Loop, Validator, Idempotent Consumer, Content Enricher) and catalog-only patterns (Claim Check, Sort, Sampling, Resequencer) in a deferred section with one-line descriptions and no dedicated pages. Content Enricher is deferred because no compiled `.enrich()` example exists.

#### Scenario: deferred section lists all catalog entries

- **GIVEN** the rewritten `eip/index.md`
- **WHEN** the user reads the deferred patterns section
- **THEN** ten pattern names appear with one-line descriptions
- **AND** none of them link to dedicated pages

### Requirement: SUMMARY lists all EIP pages

The `docs/src/SUMMARY.md` file SHALL list all 17 EIP pages (3 existing plus 14 new) under the EIP section.

#### Scenario: SUMMARY contains 17 EIP entries

- **GIVEN** the updated `docs/src/SUMMARY.md`
- **WHEN** a user or tool reads the EIP section
- **THEN** 17 page entries appear, one per pattern page

### Requirement: Guide builds and links resolve

The mdBook guide SHALL build with zero errors after adding the 14 new pages, and all cross-page links SHALL resolve without dangling references.

#### Scenario: mdbook build succeeds

- **GIVEN** all 14 new EIP pages and the rewritten hub
- **WHEN** `mdbook build docs` runs
- **THEN** the build exits with code 0

#### Scenario: no dangling links

- **GIVEN** the built guide
- **WHEN** a link checker scans all pages
- **THEN** zero dangling references are found

### Requirement: All modified examples compile

Every example that receives anchor comments SHALL compile without errors, ensuring that the include blocks reference valid, compilable code.

#### Scenario: all anchored examples build

- **GIVEN** all 14 existing examples that gain anchor comments
- **WHEN** `cargo build` runs on each example crate
- **THEN** each build succeeds with zero errors
