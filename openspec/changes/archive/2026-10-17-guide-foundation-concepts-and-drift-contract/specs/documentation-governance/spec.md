## ADDED Requirements

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

### Requirement: Mechanical slop markers are advisory-checked

The `lint-slop` xtask SHALL scan `docs/src/**/*.md` for mechanical slop markers (em-dash `—`; banned verbs `leverage`, `utilize`, `facilitate`, `prior to`; buzzwords `seamless`, `robust`, `powerful`) and report each hit. Content inside fenced code blocks SHALL be exempt. The check is advisory by default (reports, exit 0) and blocking only under `--deny` or at the 1.0 gate.

#### Scenario: slop markers present

- **GIVEN** `docs/src/getting-started/installation.md` contains the word `seamless` outside a code fence
- **WHEN** `cargo xtask lint-slop` runs
- **THEN** it reports the hit (advisory exit 0; non-zero under `--deny`)

#### Scenario: code-fence content is exempt

- **GIVEN** a code fence in a guide page contains an em-dash as part of a command example
- **WHEN** `cargo xtask lint-slop` runs
- **THEN** it does NOT report the em-dash, because fenced content is exempt

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
