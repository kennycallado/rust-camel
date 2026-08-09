# documentation-governance Specification

## Purpose
TBD - created by archiving change context-map-guide-refresh-clause. Update Purpose after archive.
## Requirements
### Requirement: User-visible-contract guide refresh trigger

The project SHALL treat a user-visible contract change as a documentation-refresh trigger: when an architecture-shaping merge changes a user-visible contract, the change SHALL also refresh the affected mdBook guide section and its anchored `examples/` include, in the same change.

A user-visible contract change is one of: a new EIP builder method, a new component scheme, a DSL key rename, a lifecycle-state rename, or a public contract enum gaining a variant.

This rule extends the existing event-driven refresh contract (CONTEXT-MAP.md, "Refresh is event-driven", bullet 1). It does not apply to internal refactors that leave all user-visible contracts unchanged.

#### Scenario: new EIP builder method added

- **GIVEN** a merge adds a new EIP builder method to `RouteBuilder` (for example `.resequence(...)`)
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide section documenting EIP patterns and the `examples/` file whose anchor demonstrates the new method are both updated in the same change

#### Scenario: public contract enum gains a variant

- **GIVEN** a merge adds a variant to a public contract enum that is `#[non_exhaustive]` (per ADR-0049), such as `RuntimeCommand`
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide section that documents matching on that enum is updated in the same change, and the anchored `examples/` include that demonstrates the match is updated to match

#### Scenario: internal refactor with no user-visible change

- **GIVEN** a merge reorganizes internal module boundaries inside `camel-core` (per ADR-0045) without adding, renaming, or removing any user-visible contract
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide is NOT required to change (only CONTEXT-MAP Contexts/Relationships and touched CONTEXT.md files refresh, per the existing bullet)

### Requirement: Two-source guide authoring rule

The mdBook guide SHALL author concept pages by the two-source rule: each statement of current *form* (a type, signature, key, flag, enum variant) SHALL be demonstrated by an anchored `{{#include}}` from a compiled example, and each statement of design *rationale* SHALL cite the governing ADR with at most a one-sentence paraphrase. The guide SHALL NOT define domain terms (CONTEXT-MAP owns definitions) and SHALL NOT restate ADR reasoning.

This rule is recorded in `docs/src/contributing.md` (the Documentation workflow section) as the project authoring rule "Show the form, cite the decision."

#### Scenario: concept page demonstrates form via include

- **GIVEN** a concept page under `docs/src/concepts/` makes a claim about current code form (for example, the `RouteBuilder::from(...).to(...)` shape)
- **WHEN** the page is reviewed
- **THEN** the claim is accompanied by an `{{#include}}` from a compiled example on the same page, so `mdbook test` + `cargo check` fail if the form drifts

#### Scenario: concept page cites ADR for rationale

- **GIVEN** a concept page explains a design choice (for example, Stop is successful control flow, not an error)
- **WHEN** the page is reviewed
- **THEN** the page links the governing ADR (ADR-0024) and gives at most a one-sentence paraphrase; it does not reproduce the ADR's reasoning

#### Scenario: form-claim without adjacent include is a defect

- **GIVEN** a concept page asserts a form claim with no `{{#include}}` on the same page
- **WHEN** the page is reviewed
- **THEN** the claim is treated as a drift defect to fix (add the include) or remove

### Requirement: Guide glossary is a curated projection of CONTEXT-MAP

The guide glossary (`docs/src/concepts/glossary.md`) SHALL be a curated subset of CONTEXT-MAP Key Terms. Every bold canonical term (`**Term**`) in the glossary SHALL match a CONTEXT-MAP Key Term — either exactly, or as the first component of a compound Key Term (so `**ErrorHandler**` matches the `ErrorHandler / ErrorHandlerConfig / ExceptionPolicy` entry). The `lint-glossary` xtask SHALL enforce this: it reports a violation for any glossary bold term whose first component is not any Key Term's first component. The glossary MAY rephrase a definition for users (wording drift is acceptable); it SHALL NOT invent terms (vocabulary drift is blocked).

#### Scenario: glossary term exists in CONTEXT-MAP

- **GIVEN** the glossary contains `**PollingConsumer**` as a bold term
- **WHEN** `cargo xtask lint-glossary` runs
- **THEN** it exits 0, because `PollingConsumer` is a Key Term in `CONTEXT-MAP.md`

#### Scenario: glossary term matches a compound Key Term by first component

- **GIVEN** the glossary contains `**ErrorHandler**` as a bold term
- **WHEN** `cargo xtask lint-glossary` runs
- **THEN** it exits 0, because `ErrorHandler` is the first component of the compound Key Term `ErrorHandler / ErrorHandlerConfig / ExceptionPolicy`

#### Scenario: glossary term absent from CONTEXT-MAP

- **GIVEN** the glossary contains `**FooBar**` as a bold term that is not any Key Term or any compound Key Term's first component
- **WHEN** `cargo xtask lint-glossary` runs
- **THEN** it reports a violation naming `FooBar` and exits non-zero under `--deny` (advisory exit 0 by default)

#### Scenario: foundational primitives are cited, not lint-checked

- **GIVEN** the glossary lists `Route` as a non-bold entry citing `crates/camel-core/CONTEXT.md`
- **WHEN** `cargo xtask lint-glossary` runs
- **THEN** it does NOT flag `Route` (non-bold primitives are exempt; they are crate-local terms owned by contract/runtime crates, not CONTEXT-MAP cross-cutting Key Terms)

### Requirement: ADR citations in the guide are valid

Every `ADR-00NN` reference in `docs/src/**/*.md` SHALL resolve to an existing file `docs/adr/00NN-*.md` whose status is not `Retired` or `Superseded`. The `lint-adr-cite` xtask SHALL determine status by parsing any status line in the supported on-disk formats (`Status:`, `**Status:**`, `**Status**:`, `## Status`); a statusless legacy ADR (no status line) SHALL be treated as active and not a violation. A retired ADR (notably ADR-0048) SHALL NOT be cited as active guidance.

#### Scenario: cited ADR exists and is active

- **GIVEN** a guide page cites `ADR-0024`
- **WHEN** `cargo xtask lint-adr-cite` runs
- **THEN** it exits 0, because `docs/adr/0024-*.md` exists and its Status is not Retired/Superseded

#### Scenario: cited ADR is retired

- **GIVEN** a guide page cites `ADR-0048` (Retired)
- **WHEN** `cargo xtask lint-adr-cite` runs
- **THEN** it reports a violation naming ADR-0048 as retired

#### Scenario: cited ADR file is missing

- **GIVEN** a guide page cites `ADR-0099` and no `docs/adr/0099-*.md` exists
- **WHEN** `cargo xtask lint-adr-cite` runs
- **THEN** it reports a violation naming ADR-0099 as unresolved

#### Scenario: statusless legacy ADR is treated as active

- **GIVEN** a guide page cites `ADR-0001`, whose file has no `Status:` line (legacy format)
- **WHEN** `cargo xtask lint-adr-cite` runs
- **THEN** it does NOT report a violation (statusless ADRs are treated as active; the linter logs a note)

### Requirement: Guide prose follows ASD-STE100

Guide prose SHALL follow ASD-STE100 Simplified Technical English. Agents editing `docs/src/**/*.md` SHALL load the `ste-writing` skill before authoring or revising prose. There is no mechanical prose linter; quality depends on the skill and expert review.

#### Scenario: agent edits documentation prose

- **GIVEN** an agent is about to edit a `.md` file in `docs/src/`
- **WHEN** the agent reads `docs/AGENTS.md`
- **THEN** the agent loads the `ste-writing` skill and applies ASD-STE100 rules to all prose changes

### Requirement: Guide concept structure

The `docs/src/concepts/` section SHALL be a navigable structure: an `index.md` hub plus five sub-pages (`exchange-message.md`, `routes-pipelines.md`, `components-endpoints.md`, `planes.md`, `glossary.md`). `docs/src/SUMMARY.md` SHALL list the index and all five sub-pages under the `Core concepts` entry. `docs/src/introduction.md` SHALL contain a one-paragraph "coming from Apache Camel?" note citing ADR-0046.

#### Scenario: SUMMARY lists the concept sub-pages

- **GIVEN** `docs/src/SUMMARY.md` after the change
- **WHEN** the `Core concepts` entry is reviewed
- **THEN** it links `concepts/index.md` plus the five sub-page entries (Exchange & Message, Routes & pipelines, Components & endpoints, Data plane vs control plane, Glossary)

#### Scenario: each concept page exists and follows the two-source rule

- **GIVEN** the five concept sub-pages exist under `docs/src/concepts/`
- **WHEN** each page is reviewed
- **THEN** every page that makes a form-claim carries an `{{#include}}` on the same page, and every design-rationale statement cites a governing ADR (per the two-source rule requirement)

#### Scenario: introduction carries the Camel-divergence note

- **GIVEN** `docs/src/introduction.md` after the change
- **WHEN** the page is reviewed
- **THEN** it contains a one-paragraph note stating rust-camel is Apache-Camel-inspired (not drop-in) and cites ADR-0046

### Requirement: Two-source rule extends to recipe and operations pages

The system SHALL apply the two-source authoring rule to every form-bearing page under `docs/src/eip/`, `docs/src/components/`, `docs/src/yaml-dsl/`, `docs/src/operations/`, and `docs/src/extending/`, identical to the rule established for `docs/src/concepts/`.

#### Scenario: EIP pattern page makes a form claim

- **GIVEN** a page under `docs/src/eip/` demonstrates a pattern such as the Message Filter `.filter()` verb
- **WHEN** the page is reviewed
- **THEN** the claim is accompanied by an `{{#include}}` of the anchored region from the corresponding compiled example (for example `examples/content-based-routing/src/main.rs:filter-route`), so the shown form is compiler-checked

#### Scenario: component page shows a URI scheme

- **GIVEN** a page under `docs/src/components/` shows a component URI such as `file:{input}?delete=true`
- **WHEN** the page is reviewed
- **THEN** the URI appears inside an `{{#include}}` from a compiled example, not as hand-typed prose

#### Scenario: page restates ADR reasoning

- **GIVEN** any new page explains a design choice
- **WHEN** the page is reviewed
- **THEN** it links the governing ADR and gives at most a one-sentence paraphrase; it does not reproduce the ADR's reasoning

### Requirement: Section index pages are navigation hubs

The system SHALL make each section index page (`eip/index.md`, `components/index.md`, `yaml-dsl/index.md`, `operations/index.md`, `extending/index.md`, `architecture/index.md`) a navigation hub: a one-paragraph frame followed by categorized links to runnable examples and per-crate `CONTEXT.md` files. A hub owns no form-claims (no `{{#include}}`, no inline code fence) and no ADR paraphrase; it routes.

#### Scenario: reader lands on a section index

- **GIVEN** a reader opens `docs/src/eip/index.md`
- **WHEN** the page renders
- **THEN** it presents categorized links to the EIP pattern pages and to runnable examples, with no inline code fence duplicating example content

#### Scenario: hub contains a form claim

- **GIVEN** a section index page contains an `{{#include}}` or a hand-typed code fence asserting code form
- **WHEN** the page is reviewed
- **THEN** the claim is treated as a defect (hubs route; foundation pages claim)

#### Scenario: component catalog URI rows

- **GIVEN** the components index lists URI schemes (`timer:`, `log:`, `file:`) in its catalog table
- **WHEN** the page is reviewed
- **THEN** each row links the component's `CONTEXT.md` (the authority for the scheme); the URI text is reference data mapping to that authority, not a compiled-form claim requiring an include

### Requirement: Anchored example regions compile

The system SHALL keep every example file that receives a new `ANCHOR`/`ANCHOR_END` comment pair compilable. Anchor comments are inserted between existing lines and change no behavior. Rust files use `// ANCHOR:`; YAML files use `# ANCHOR:`.

#### Scenario: anchor added to a Rust example

- **GIVEN** an `ANCHOR: filter-route` pair is added to `examples/content-based-routing/src/main.rs`
- **WHEN** `cargo check -p content-based-routing` runs
- **THEN** it succeeds (the comments are inert)

#### Scenario: anchor added to a YAML route

- **GIVEN** an `# ANCHOR: hot-reload-route` pair is added to `examples/hot-reload/routes/route.yaml`
- **WHEN** the example's config is loaded
- **THEN** it parses (the comments are inert YAML comments)

#### Scenario: anchor referenced by a page

- **GIVEN** a page includes `{{#include ../../../examples/circuit-breaker/src/main.rs:circuit-breaker-route}}`
- **WHEN** `nix shell nixpkgs#mdbook -c mdbook build docs` runs
- **THEN** the build succeeds and the include resolves (no missing-anchor error)

### Requirement: Foundation page inventory and SUMMARY wiring

The system SHALL deliver the following foundation pages and wire each as a nested sub-page entry in `docs/src/SUMMARY.md` under its section heading, mirroring the Core-concepts nesting pattern from the prior change:

- `eip/filter.md`, `eip/circuit-breaker.md`, `eip/aggregator.md`
- `components/timer-log.md`, `components/file.md`, `components/http.md`
- `yaml-dsl/route-structure.md`
- `operations/health.md`
- `extending/custom-component.md`

(Architecture is index-only; no foundation sub-pages this change.)

#### Scenario: SUMMARY lists all foundation pages

- **GIVEN** the foundation pages are authored and wired
- **WHEN** each of the nine page paths is checked: `rg -F -c 'eip/filter.md' docs/src/SUMMARY.md` (and the same for `eip/circuit-breaker.md`, `eip/aggregator.md`, `components/timer-log.md`, `components/file.md`, `components/http.md`, `yaml-dsl/route-structure.md`, `operations/health.md`, `extending/custom-component.md`)
- **THEN** each check returns `1` (the path appears exactly once as a nested sub-page entry under its section heading)

#### Scenario: a promised page is missing

- **GIVEN** a foundation page listed above is absent from `docs/src/`
- **WHEN** the change is reviewed
- **THEN** it is treated as an incomplete deliverable

### Requirement: Component catalog is a map to authorities

The system SHALL present the components index as a catalog table mapping each component's URI scheme to its owning crate and a link to the component's local `CONTEXT.md` or its nearest parent (`components/CONTEXT.md`) per the coverage policy, without restating component definitions.

#### Scenario: reader looks up a component

- **GIVEN** a reader opens `docs/src/components/index.md`
- **WHEN** the page renders
- **THEN** it shows a table with at least the columns URI-scheme, direction (source/sink/both), and a link to the component's local `CONTEXT.md` or its nearest parent (`components/CONTEXT.md`) per the coverage policy

#### Scenario: catalog restates a definition

- **GIVEN** the components index restates a component's behavioral definition instead of linking its `CONTEXT.md`
- **WHEN** the page is reviewed
- **THEN** the restatement is treated as a drift risk to fix (replace with a link)

### Requirement: Architecture index is a crate map

The system SHALL present `docs/src/architecture/index.md` as a crate map: a table of every crate, with a one-line role and a link to the crate's local `CONTEXT.md` if it has one, or its nearest parent `CONTEXT.md` per the coverage policy. The page also points to the ADR index and links back to `concepts/planes.md`. The page does not assert the `Service<Exchange>` trait signature as a form-claim.

#### Scenario: reader navigates the crate structure

- **GIVEN** a reader opens `docs/src/architecture/index.md`
- **WHEN** the page renders
- **THEN** it links each crate's local `CONTEXT.md` if it has one, or its nearest parent `CONTEXT.md` per the coverage policy, plus the ADR directory, with no hand-typed trait signature

#### Scenario: page asserts the trait signature

- **GIVEN** the architecture index writes a code fence asserting `Service<Exchange>` as current form
- **WHEN** the page is reviewed
- **THEN** the claim is treated as a defect (no compiled example provides it; cite ADR-0001 as rationale instead)

### Requirement: Guide builds and links resolve

The system SHALL build the complete mdBook without errors AND verify that every relative Markdown link resolves to an existing file (mdbook build emits broken-link warnings but does not fail the build on them, so an explicit check is required).

#### Scenario: full guide build

- **GIVEN** all new pages and anchors are in place
- **WHEN** `nix shell nixpkgs#mdbook -c mdbook build docs` runs
- **THEN** it exits 0 and the output contains no `broken link` or `missing file` warnings

#### Scenario: relative link verification

- **GIVEN** new pages contain relative `[text](page.md)` links
- **WHEN** this check runs and produces empty output:
  ```bash
  find docs/src -name '*.md' -print0 | while IFS= read -r -d '' f; do
    dir=$(dirname "$f")
    rg -No '\]\(([^)]+\.md)\)' -r '$1' "$f" 2>/dev/null | while IFS= read -r target; do
      case "$target" in http* | /*) continue ;; esac
      [ -f "$dir/$target" ] || echo "DANGLING: $f -> $target"
    done
  done
  ```
- **THEN** the output is empty (zero dangling links)

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

### Requirement: All modified examples compile

Every example that receives anchor comments SHALL compile without errors, ensuring that the include blocks reference valid, compilable code.

#### Scenario: all anchored examples build

- **GIVEN** all 14 existing examples that gain anchor comments
- **WHEN** `cargo build` runs on each example crate
- **THEN** each build succeeds with zero errors

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

