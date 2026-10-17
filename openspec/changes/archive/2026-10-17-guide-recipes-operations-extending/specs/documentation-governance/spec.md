## ADDED Requirements

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
